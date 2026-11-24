//! The files `prepare` puts on a new guest's disk: an answer file that takes Windows past its first
//! questions with a local account, the script Windows runs when it has set itself up, and the report
//! that script schedules.

/// Run as SYSTEM at every start, it writes what the guest sees of its GPU to a file and to COM1.
pub const REPORT: &str = include_str!("../guest/report.ps1");
/// The report's file, from the root of the guest's Windows volume, which `boot` reads once the
/// machine has stopped. `report.ps1` names it as `C:\Guesthouse\report.txt`.
pub const REPORT_FILE: &str = r"Guesthouse\report.txt";
/// The report's last line, on either system's serial port.
pub const REPORT_DONE: &str = "guesthouse: report done";

/// What the host writes on a guest's serial port to ask it to shut down. HCS's own request needs
/// Hyper-V's integration services in the guest, which a machine HCS makes doesn't have, so the guest
/// listens for this and shuts itself down, as App Sandbox's guest agent does when asked.
/// `report.ps1` listens for it in Windows and `shutdown.sh` in Ubuntu.
pub const SHUTDOWN_LINE: &str = "guesthouse: shutdown";

/// Whether a line from the guest is its report's last.
pub fn report_done(line: &str) -> bool {
    line.starts_with(REPORT_DONE)
}

/// Where the report goes, from the root of the guest's Windows volume. `SetupComplete.cmd` names it
/// as `C:\Guesthouse\report.ps1`.
pub const REPORT_PATH: &str = r"Guesthouse\report.ps1";
pub const SETUP_COMPLETE: &str = include_str!("../guest/SetupComplete.cmd");
/// Windows runs this once its own setup is done.
pub const SETUP_COMPLETE_PATH: &str = r"Windows\Setup\Scripts\SetupComplete.cmd";
/// Windows reads the answer file here on the first start of an image applied with no Setup.
pub const ANSWER_FILE_PATH: &str = r"Windows\Panther\unattend.xml";

const COMPONENT: &str = r#"processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS""#;

/// An answer file for the oobeSystem pass: English (US), every first-start question skipped, and one
/// local administrator.
/// The accounts every Windows has. The answer file's account takes the existing one when the name
/// matches, whatever its case, and those are disabled, so none can be the guest's administrator.
const WINDOWS_ACCOUNTS: [&str; 4] = ["Administrator", "DefaultAccount", "Guest", "WDAGUtilityAccount"];

/// Whether `user` is one of Windows' own accounts, and so can't be made as a new one.
pub fn windows_account(user: &str) -> bool {
    WINDOWS_ACCOUNTS.iter().any(|account| account.eq_ignore_ascii_case(user))
}

pub fn answer_file(user: &str, password: &str) -> String {
    let (user, password) = (escape(user), escape(password));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" {COMPONENT}>
      <InputLocale>en-US</InputLocale><SystemLocale>en-US</SystemLocale><UILanguage>en-US</UILanguage><UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" {COMPONENT}>
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <ProtectYourPC>3</ProtectYourPC>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>{user}</Name>
            <Group>Administrators</Group>
            <Password><Value>{password}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
    </component>
  </settings>
</unattend>
"#
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_own_accounts_are_known_whatever_their_case() {
        assert!(windows_account("guest"));
        assert!(windows_account("ADMINISTRATOR"));
        assert!(!windows_account("guesthouse"));
    }

    #[test]
    fn the_answer_file_makes_the_named_administrator() {
        let file = answer_file("guest", "p&ss<word>");
        assert!(file.contains("<Name>guest</Name>"));
        assert!(file.contains("<Value>p&amp;ss&lt;word&gt;</Value>"));
        assert!(file.contains("<Group>Administrators</Group>"));
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
    /// What the guest says on its serial port when its install is done, as `user-data` writes it.
    pub const INSTALL_FINISHED: &str = "guesthouse-install-finished";
    /// The guest's GPU files on the files disk, which it mounts at /usr/lib/wsl.
    pub const WSL_LIB: &str = r"wsl\lib";
    pub const WSL_DRIVERS: &str = r"wsl\drivers";
    /// The file install.sh makes the guest's administrator from, and then deletes.
    pub const ACCOUNT: &str = r"guesthouse\account";

    const GRUB: &str = include_str!("../guest/linux/grub.cfg");

    /// Files written to the files disk, by their path on it. The boot menu is in each place
    /// Ubuntu's GRUB may look for it, by the folder it started from or by its own search.
    pub const FILES: [(&str, &str); 18] = [
        (r"EFI\BOOT\grub.cfg", GRUB),
        (r"EFI\ubuntu\grub.cfg", GRUB),
        (r"boot\grub\grub.cfg", GRUB),
        (r".disk\info", "Guesthouse's files disk\n"),
        ("user-data", include_str!("../guest/linux/user-data")),
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

    /// Whether a line from the guest is the one saying its install is done. Only a line that starts
    /// with it counts, so the installer showing the command that says it doesn't.
    pub fn install_finished(line: &str) -> bool {
        line.starts_with(INSTALL_FINISHED)
    }

    pub fn account(user: &str, password: &str) -> String {
        format!("{user} {password}\n")
    }

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
