//! The files `prepare` puts on a new guest's disk: an answer file that takes Windows past its first
//! questions with a local account, the script Windows runs when it has set itself up, and the report
//! that script schedules.

/// Run as SYSTEM at every start, it writes what the guest sees of its GPU to a file and to COM1.
pub const REPORT: &str = include_str!("../guest/report.ps1");
/// The report's file, from the root of the guest's Windows volume, which `boot` reads once the
/// machine has stopped. `report.ps1` names it as `C:\Guesthouse\report.txt`.
pub const REPORT_FILE: &str = r"Guesthouse\report.txt";
/// The report's last line, on either system's serial port, and whether a line from the guest is it.
pub use guesthouse_serial_words_core::windows_host::{REPORT_DONE, report_done};

/// What `Guesthouse\share.txt` holds when a Windows guest has shares: a line for each, its name, the
/// port of the host's Plan 9 server it reaches it on, and `rw` or `ro`, as `shared 50001 rw`.
/// `report.ps1` reads them and starts `guesthouse-guest-share.exe` with each line's.
pub fn share_marker(shares: &[crate::settings::Share]) -> String {
    shares
        .iter()
        .map(|share| {
            format!(
                "{} {} {}\r\n",
                share.name(),
                guesthouse_hcs::schema::PLAN9_WINDOWS_PORT,
                if share.read_only { "ro" } else { "rw" }
            )
        })
        .collect()
}

/// What the host writes on a guest's serial port to ask it to shut down. `report.ps1` listens for it
/// in Windows and `shutdown.sh` in Ubuntu.
pub use guesthouse_serial_words_core::windows_host::SHUTDOWN_LINE;

/// Where the report goes, from the root of the guest's Windows volume. `SetupComplete.cmd` names it
/// as `C:\Guesthouse\report.ps1`.
pub const REPORT_PATH: &str = r"Guesthouse\report.ps1";
/// Where the host says who may reach the ports a Windows guest lets in, Remote Desktop's and its
/// screen's, from the root of its Windows volume. `report.ps1` names it as `C:\Guesthouse\hosts.txt`.
/// Written before each start; with none, the guest lets nobody in by those rules.
pub const HOSTS_PATH: &str = r"Guesthouse\hosts.txt";

/// What [`HOSTS_PATH`] holds: the network the machine is on, `shared` or `bridged`, then this PC's
/// addresses, a line each. On the shared network the guest also lets in its default gateway, which
/// is this PC on that network, where port forwards reach it from, and may not have an address yet
/// when the file is written, as Windows makes the network's adapter as the machine starts.
pub fn hosts_file(bridged: bool, addresses: &[std::net::Ipv4Addr]) -> String {
    let mut text = String::from(if bridged { "bridged\n" } else { "shared\n" });
    for address in addresses {
        text.push_str(&format!("{address}\n"));
    }
    text
}

pub const SETUP_COMPLETE: &str = include_str!("../guest/SetupComplete.cmd");
/// Windows runs this once its own setup is done.
pub const SETUP_COMPLETE_PATH: &str = r"Windows\Setup\Scripts\SetupComplete.cmd";
/// Windows reads the answer file here on the first start of an image applied with no Setup.
pub const ANSWER_FILE_PATH: &str = r"Windows\Panther\unattend.xml";

/// An answer file for the oobeSystem pass: English (US), every first-start question skipped, and one
/// local administrator. The Windows host's, in the shared crate.
pub use guesthouse_answers_core::windows_host::answer_file;
/// The Windows host's XML escape, which leaves the apostrophe as it is.
#[cfg(test)]
use guesthouse_answers_core::windows_host::escape;

#[cfg(test)]
#[path = "guest_golden.rs"]
mod answers_golden;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_file_makes_the_named_administrator() {
        let file = answer_file("guest", "p&ss<word>");
        assert!(file.contains("<Name>guest</Name>"));
        assert!(file.contains("<Value>p&amp;ss&lt;word&gt;</Value>"));
        assert!(file.contains("<Group>Administrators</Group>"));
    }

    /// The account rules take capitals, dots and spaces between words in a Windows guest's name,
    /// which go into the file as they are.
    #[test]
    fn a_name_with_capitals_dots_and_spaces_is_written_as_it_is() {
        let file = answer_file("Ann B.Lee", "p");
        assert!(file.contains("<Name>Ann B.Lee</Name>"));
    }

    #[test]
    fn both_reports_end_with_the_line_boot_waits_for() {
        let done = REPORT_DONE.trim_start_matches("guesthouse: ");
        assert!(REPORT.contains(&format!("Send-Line '{done}'")));
        let gpu = linux::FILES.iter().find(|(path, _)| path.ends_with("gpu.sh")).unwrap().1;
        assert!(gpu.contains(&format!("say \"{done}\"")));
        assert!(report_done("guesthouse: report done"));
        assert!(!report_done("say \"guesthouse: report done\""));
    }

    #[test]
    fn the_share_marker_is_a_line_a_share_the_report_reads_as_name_port_and_mode() {
        use crate::settings::Share;
        let share = |slot, read_only| Share { folder: "D:\\x".into(), read_only, slot };
        assert_eq!(share_marker(&[share(0, false)]), "shared 50001 rw\r\n");
        assert_eq!(
            share_marker(&[share(0, true), share(2, false)]),
            "shared 50001 ro\r\nshared3 50001 rw\r\n"
        );
        assert_eq!(share_marker(&[]), "");
        // The report splits each line on white space and takes those three, as the host wrote them,
        // and takes the slot from the name.
        assert!(REPORT.contains("-split '\\s+'"));
        assert!(REPORT.contains(r"C:\Guesthouse\share.txt"));
        assert!(REPORT.contains("-eq 'ro'"));
        assert!(REPORT.contains("'^shared(\\d*)$'"));
        // And what it runs and installs is where the host puts it.
        assert!(REPORT.contains(r"$shareFolder = 'C:\Guesthouse\share'"));
        assert!(REPORT.contains("'guesthouse-guest-share.exe'"));
        assert!(REPORT.contains("'winfsp.msi'"));
    }

    #[test]
    fn the_guest_is_told_its_network_and_this_pcs_addresses_a_line_each() {
        use std::net::Ipv4Addr;
        let addresses = [Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(172, 30, 0, 1)];
        assert_eq!(hosts_file(true, &addresses), "bridged\n192.168.1.20\n172.30.0.1\n");
        assert_eq!(hosts_file(false, &[]), "shared\n");
        assert!(REPORT.contains(&format!(r"'C:\{HOSTS_PATH}'")), "the guest reads the file by this name");
    }

    /// Remote Desktop is let in by the report's own rule, at every start, and SetupComplete.cmd turns
    /// on no firewall rule of Windows' own.
    #[test]
    fn remote_desktop_is_let_in_by_the_reports_rule_alone() {
        assert!(SETUP_COMPLETE.contains("fDenyTSConnections"));
        assert!(!SETUP_COMPLETE.to_lowercase().contains("advfirewall"), "{SETUP_COMPLETE}");
        assert!(REPORT.contains("'@FirewallAPI.dll,-28752'"));
        assert!(REPORT.contains("Get-DesktopAddresses $hostsFile $hostAddresses"));
        assert!(REPORT.contains("Set-RemoteDesktop $desktopAddresses 'Guesthouse Remote Desktop'"));
    }

    #[test]
    fn the_scripts_agree_on_where_the_report_is() {
        assert!(SETUP_COMPLETE.contains(&format!(r"C:\{REPORT_PATH}")));
        assert!(REPORT.contains("COM1"));
        assert!(REPORT.contains(&format!(r"C:\{REPORT_FILE}")));
    }
}

/// What a Linux guest's files disk holds for its install and its GPU.
pub mod linux {
    /// The files disk's FAT label, which is also how Ubuntu's installer finds its answer file there.
    pub const LABEL: &str = "CIDATA";
    /// What the guest says on its serial port when its install is done, as `user-data` writes it,
    /// and whether a line from the guest is it.
    pub use guesthouse_serial_words_core::windows_host::linux::{INSTALL_FINISHED, install_finished};
    /// The guest's GPU files on the files disk, which it mounts at /usr/lib/wsl.
    pub const WSL_LIB: &str = r"wsl\lib";
    pub const WSL_DRIVERS: &str = r"wsl\drivers";
    /// The file install.sh makes the guest's administrator from. The guest has the files disk
    /// read-only, so the host takes it off at the first start after the install.
    pub const ACCOUNT: &str = r"guesthouse\account";
    /// Who besides the loopback may sign in to the guest's sshd with a password, as
    /// [`super::hosts_file`] writes it:
    /// `gpu.sh` reads it at every start with `ssh-from.sh`.
    pub const HOSTS: &str = r"guesthouse\hosts.txt";
    /// Ubuntu's installer's answer file, which the host takes off with the account, so the installed
    /// system never takes it for its own.
    pub const USER_DATA: &str = "user-data";

    const GRUB: &str = include_str!("../guest/linux/grub.cfg");

    /// Files written to the files disk, by their path on it. The boot menu is in each place
    /// Ubuntu's GRUB may look for it, by the folder it started from or by its own search.
    pub const FILES: [(&str, &str); 19] = [
        (r"EFI\BOOT\grub.cfg", GRUB),
        (r"EFI\ubuntu\grub.cfg", GRUB),
        (r"boot\grub\grub.cfg", GRUB),
        (r".disk\info", "Guesthouse's files disk\n"),
        (USER_DATA, include_str!("../guest/linux/user-data")),
        ("meta-data", include_str!("../guest/linux/meta-data")),
        (r"guesthouse\install.sh", include_str!("../guest/linux/install.sh")),
        (r"guesthouse\start.sh", include_str!("../guest/linux/start.sh")),
        (r"guesthouse\gpu.sh", include_str!("../guest/linux/gpu.sh")),
        (r"guesthouse\guesthouse-gpu.service", include_str!("../guest/linux/guesthouse-gpu.service")),
        (r"guesthouse\shutdown.sh", include_str!("../guest/linux/shutdown.sh")),
        (
            r"guesthouse\guesthouse-shutdown.service",
            include_str!("../guest/linux/guesthouse-shutdown.service"),
        ),
        (r"guesthouse\guesthouse-ssh.socket", include_str!("../guest/linux/guesthouse-ssh.socket")),
        (r"guesthouse\guesthouse-ssh.service", include_str!("../guest/linux/guesthouse-ssh.service")),
        (r"guesthouse\guesthouse-hv_sock.conf", include_str!("../guest/linux/guesthouse-hv_sock.conf")),
        (r"guesthouse\ssh-from.sh", include_str!("../guest/linux/ssh-from.sh")),
        (r"guesthouse\dxgkrnl\build.sh", include_str!("../guest/linux/dxgkrnl/build.sh")),
        (r"guesthouse\dxgkrnl\Kbuild", include_str!("../guest/linux/dxgkrnl/Kbuild")),
        (
            r"guesthouse\dxgkrnl\guesthouse-compat.h",
            include_str!("../guest/linux/dxgkrnl/guesthouse-compat.h"),
        ),
    ];

    /// The installer's own boot files, from its ISO to the files disk. Shim starts first, as the
    /// firmware's fallback, and starts GRUB beside it, which finds the menu above.
    pub const BOOT_FILES: [(&str, &str); 3] = [
        (r"EFI\boot\bootx64.efi", r"EFI\BOOT\BOOTX64.EFI"),
        (r"EFI\boot\grubx64.efi", r"EFI\BOOT\grubx64.efi"),
        (r"EFI\boot\mmx64.efi", r"EFI\BOOT\mmx64.efi"),
    ];

    /// What marks an ISO as Ubuntu's installer: the kernel and initrd the boot menu starts.
    pub const INSTALLER_KERNEL: &str = r"casper\vmlinuz";
    pub const INSTALLER_INITRD: &str = r"casper\initrd";

    /// The line `install.sh` reads, `user password`, as the shared crate makes it.
    pub use guesthouse_answers_core::windows_host::ubuntu::account;

    /// A file as Linux needs it. A Windows checkout may have turned its line endings into CRLF,
    /// and `#!/bin/sh` with a carriage return after it runs nothing.
    pub fn unix_text(text: &str) -> String {
        text.replace("\r\n", "\n")
    }
}

#[cfg(test)]
mod linux_tests {
    use super::SHUTDOWN_LINE;
    use super::linux::*;

    #[test]
    fn the_answer_file_says_what_boot_waits_for() {
        let user_data = FILES.iter().find(|(path, _)| *path == "user-data").unwrap().1;
        assert!(user_data.contains(&format!("echo {INSTALL_FINISHED} > /dev/ttyS0")));
        assert!(user_data.contains(&format!("LABEL={LABEL}")));
    }

    #[test]
    fn the_boot_menu_starts_the_installer_unattended_on_the_serial_port_too() {
        let grub = FILES[0].1;
        assert!(grub.contains("autoinstall ds=nocloud"));
        assert!(grub.contains("console=ttyS0"));
        assert!(grub.contains(&format!("/{}", INSTALLER_KERNEL.replace('\\', "/"))));
        assert!(grub.contains(&format!("/{}", INSTALLER_INITRD.replace('\\', "/"))));
    }

    #[test]
    fn the_install_script_reads_the_account_where_it_is_written() {
        let install = FILES.iter().find(|(path, _)| path.ends_with("install.sh")).unwrap().1;
        assert!(install.contains(&format!("$files/{}", ACCOUNT.replace('\\', "/"))));
    }

    fn file(name: &str) -> &'static str {
        FILES.iter().find(|(path, _)| path.ends_with(name)).unwrap().1
    }

    /// What ssh-from.sh prints for this hosts file and these gateways, run by `sh` as gpu.sh runs
    /// it. None where there is no `sh`.
    #[cfg(unix)]
    fn ssh_from(hosts: Option<&str>, gateways: &[&str]) -> Option<String> {
        ssh_from_on(hosts, gateways, &[])
    }

    /// The same, with the guest's own networks as gpu.sh reads them.
    #[cfg(unix)]
    fn ssh_from_on(hosts: Option<&str>, gateways: &[&str], networks: &[&str]) -> Option<String> {
        let folder =
            std::env::temp_dir().join(format!("gh-ssh-from-{}-{}", std::process::id(), next_number()));
        std::fs::create_dir_all(&folder).unwrap();
        let script = folder.join("ssh-from.sh");
        std::fs::write(&script, unix_text(file("ssh-from.sh"))).unwrap();
        let hosts_file = folder.join("hosts.txt");
        if let Some(text) = hosts {
            std::fs::write(&hosts_file, text).unwrap();
        }
        let ran = std::process::Command::new("sh")
            .arg(&script)
            .arg(&hosts_file)
            .arg(gateways.join(" "))
            .arg(networks.join(" "))
            .output();
        let _ = std::fs::remove_dir_all(&folder);
        let ran = ran.ok()?;
        assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
        Some(String::from_utf8(ran.stdout).unwrap())
    }

    #[cfg(unix)]
    fn next_number() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    /// The addresses ssh-from.sh takes a password from: every other one takes keys only.
    #[cfg(unix)]
    fn only_from(said: &str) -> Vec<String> {
        let line = said.lines().find_map(|line| line.strip_prefix("Match Address *,!")).unwrap_or_default();
        assert!(
            said.contains("\n    PasswordAuthentication no\n    KbdInteractiveAuthentication no\n"),
            "{said}"
        );
        line.split(",!").map(str::to_owned).collect()
    }

    /// An Ubuntu guest's sshd takes a password from the loopback, where the host's relay over a
    /// Hyper-V socket arrives, and from this PC: its addresses, and on the shared network the guest's
    /// gateway, which is where a port forward and this PC's own SSH to the guest's address come from.
    /// A bridged guest's gateway is its router, which isn't this PC.
    #[cfg(unix)]
    #[test]
    fn ssh_takes_a_password_only_from_the_loopback_and_this_pc_on_the_shared_network() {
        let Some(said) = ssh_from(Some("shared\r\n192.168.1.20\r\nnot one; rm -rf /\r\n"), &["172.29.1.1"])
        else {
            return;
        };
        assert_eq!(only_from(&said), ["127.0.0.1", "::1", "192.168.1.20", "172.29.1.1"]);
        let bridged = ssh_from(Some("bridged\n192.168.1.20\n100.64.1.2\n"), &["192.168.1.1"]).unwrap();
        assert_eq!(only_from(&bridged), ["127.0.0.1", "::1", "192.168.1.20", "100.64.1.2"]);
    }

    /// A bridged guest also takes a password from its own networks, as other devices reach it there
    /// directly; on the shared network its network is never let in, and a network that isn't one is
    /// left out.
    #[cfg(unix)]
    #[test]
    fn a_bridged_guest_takes_a_password_from_its_own_network_too() {
        let networks = ["192.168.1.0/24", "10.1.0.0/16", "bad/24", "192.168.2.0/99", "192.168.3.0"];
        let Some(bridged) = ssh_from_on(Some("bridged\n192.168.1.20\n"), &["192.168.1.1"], &networks) else {
            return;
        };
        assert_eq!(only_from(&bridged), ["127.0.0.1", "::1", "192.168.1.20", "192.168.1.0/24", "10.1.0.0/16"]);
        let nothing_known = ssh_from_on(Some("bridged\n"), &[], &["192.168.1.0/24"]).unwrap();
        assert_eq!(only_from(&nothing_known), ["127.0.0.1", "::1", "192.168.1.0/24"]);
        let shared = ssh_from_on(Some("shared\n192.168.1.20\n"), &["172.29.1.1"], &["172.29.0.0/20"]).unwrap();
        assert_eq!(only_from(&shared), ["127.0.0.1", "::1", "192.168.1.20", "172.29.1.1"]);
    }

    /// With no address of this PC's known, only the loopback takes a password, so the relay still
    /// works.
    #[cfg(unix)]
    #[test]
    fn with_no_address_known_ssh_takes_a_password_from_the_loopback_alone() {
        for hosts in [None, Some(""), Some("bridged\n"), Some("tunnel\n192.168.1.20\n")] {
            let Some(said) = ssh_from(hosts, &["192.168.1.1"]) else { return };
            assert_eq!(only_from(&said), ["127.0.0.1", "::1"], "{hosts:?}");
        }
    }

    /// sshd itself, where this PC has one, reads the settings as meant: the loopback and this PC may
    /// use a password, any other address keys only, even with a file read after it that turns
    /// passwords on, as Ubuntu's `50-cloud-init.conf` does.
    #[cfg(unix)]
    #[test]
    fn sshd_takes_passwords_from_this_pc_alone_and_keys_from_anywhere() {
        let sshd = std::path::Path::new("/usr/sbin/sshd");
        let Some(said) = ssh_from(Some("shared\n192.168.1.20\n"), &["172.29.1.1"]).filter(|_| sshd.is_file())
        else {
            return;
        };
        let folder = std::env::temp_dir().join(format!("gh-sshd-{}-{}", std::process::id(), next_number()));
        std::fs::create_dir_all(&folder).unwrap();
        let key = folder.join("key");
        let made = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output();
        if !made.is_ok_and(|made| made.status.success()) {
            let _ = std::fs::remove_dir_all(&folder);
            return;
        }
        let drop_ins = folder.join("sshd_config.d");
        std::fs::create_dir_all(&drop_ins).unwrap();
        std::fs::write(drop_ins.join("10-guesthouse-from.conf"), &said).unwrap();
        std::fs::write(drop_ins.join("50-cloud-init.conf"), "PasswordAuthentication yes\n").unwrap();
        let config = folder.join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Include {}/*.conf\nHostKey {}\nKbdInteractiveAuthentication yes\nUsePAM no\n",
                drop_ins.display(),
                key.display()
            ),
        )
        .unwrap();
        let settings = |address: &str| {
            let ran = std::process::Command::new(sshd)
                .arg("-T")
                .arg("-f")
                .arg(&config)
                .arg("-C")
                .arg(format!("user=guesthouse,host=pc,addr={address}"))
                .output()
                .unwrap();
            assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
            let said = String::from_utf8_lossy(&ran.stdout).to_lowercase();
            ["passwordauthentication", "kbdinteractiveauthentication", "pubkeyauthentication"]
                .map(|name| said.lines().any(|line| line == format!("{name} yes")))
        };
        let answers: Vec<[bool; 3]> =
            ["127.0.0.1", "::1", "192.168.1.20", "172.29.1.1", "192.168.1.77", "fe80::5"]
                .into_iter()
                .map(settings)
                .collect();
        let _ = std::fs::remove_dir_all(&folder);
        let password = [true, true, true];
        let keys_only = [false, false, true];
        assert_eq!(answers, [password, password, password, password, keys_only, keys_only]);
    }

    #[test]
    fn gpu_sh_keeps_who_may_use_a_password_from_the_hosts_file() {
        let gpu = file("gpu.sh");
        assert!(gpu.contains(&format!("\"$files/{}\"", HOSTS.replace('\\', "/"))), "{gpu}");
        assert!(gpu.contains("sh \"$here/ssh-from.sh\""));
        assert!(gpu.contains("ssh_from=/etc/ssh/sshd_config.d/10-guesthouse-from.conf"));
        assert!(gpu.contains("if said=$(sshd -t 2>&1); then"), "settings sshd won't read are never kept");
    }

    #[test]
    fn the_ssh_socket_listens_on_vsock_port_22_and_its_service_proxies_to_sshd() {
        let socket = file("guesthouse-ssh.socket");
        assert!(socket.contains("ListenStream=vsock::22\n"));
        assert!(socket.contains("Accept=no"));
        assert!(socket.contains("WantedBy=sockets.target"));
        let service = file("guesthouse-ssh.service");
        assert!(service.contains("ExecStart=/usr/lib/systemd/systemd-socket-proxyd 127.0.0.1:22\n"));
        assert!(service.contains("Requires=guesthouse-ssh.socket"));
        assert_eq!(file("guesthouse-hv_sock.conf").lines().filter(|line| *line == "hv_sock").count(), 1);
    }

    #[test]
    fn the_ssh_port_is_the_one_the_host_dials() {
        // Port 22 as a Hyper-V service ID, which the host's relay connects to.
        let id = guesthouse_hcs::hvsock::guid_text(guesthouse_hcs::hvsock::vsock_service_id(22));
        assert_eq!(id, "{00000016-facb-11e6-bd58-64006a7986d3}");
        assert_eq!(crate::ssh::GUEST_PORT, 22);
    }

    #[test]
    fn a_new_guest_and_an_older_one_both_get_the_ssh_units() {
        let install = file("install.sh");
        let gpu = file("gpu.sh");
        for unit in ["guesthouse-ssh.socket", "guesthouse-ssh.service", "guesthouse-hv_sock.conf"] {
            assert!(install.contains(unit), "install.sh and {unit}");
            assert!(gpu.contains(unit), "gpu.sh and {unit}");
        }
        assert!(install.contains("systemctl enable guesthouse-ssh.socket"));
        assert!(gpu.contains("systemctl enable guesthouse-ssh.socket"));
    }

    #[test]
    fn only_a_line_of_its_own_says_the_install_is_done() {
        assert!(install_finished("guesthouse-install-finished"));
        assert!(!install_finished("running 'echo guesthouse-install-finished > /dev/ttyS0'"));
    }

    #[test]
    fn both_guests_wait_for_the_line_the_host_writes() {
        let ubuntu = FILES.iter().find(|(path, _)| path.ends_with("shutdown.sh")).unwrap().1;
        assert!(ubuntu.contains(&format!("ask=\"{SHUTDOWN_LINE}\"")));
        assert!(ubuntu.contains("/dev/ttyS0"));
        assert!(ubuntu.contains("systemctl poweroff"));
        assert!(crate::guest::REPORT.contains(&format!("-eq '{SHUTDOWN_LINE}'")));
    }

    #[test]
    fn the_shutdown_listener_is_a_service_the_guest_scripts_enable() {
        let find = |name: &str| FILES.iter().find(|(path, _)| path.ends_with(name)).unwrap().1;
        let service = find("guesthouse-shutdown.service");
        assert!(service.contains("ExecStart=/bin/sh /usr/lib/guesthouse/shutdown.sh"));
        assert!(service.contains("Restart=always"));
        for script in ["install.sh", "gpu.sh"] {
            assert!(
                find(script).contains("systemctl enable /usr/lib/guesthouse/guesthouse-shutdown.service")
            );
        }
    }

    #[test]
    fn line_endings_become_linuxs() {
        assert_eq!(unix_text("#!/bin/sh\r\necho hi\r\n"), "#!/bin/sh\necho hi\n");
    }
}
