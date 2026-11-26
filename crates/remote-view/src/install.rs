//! remoteman's host for viewing from other devices, as a host with no viewer of its own in the guest
//! puts it there: the installer a host pins, what decides whether it has to run again, and the scripts the
//! guest runs as SYSTEM.
//!
//! Each run of the installer blips the guest's local picture for about two seconds, so it runs only
//! when something it would change changed: the version, the encoder, or the host not being there.
//! What was installed is noted in the guest, beside the installer's log.
//!
//! It is installed with no trust list (`--trust`) and no pairing (`--no-pair`): a device is only let
//! in when a person accepts it, and an installer run never replaces the devices paired since.

/// Where the guest's notes, log and installer for this live.
pub const GUEST_FOLDER: &str = r"C:\Guesthouse\remote-view";
/// The installer, where the guest is sent it.
pub const GUEST_INSTALLER: &str = r"C:\Guesthouse\remote-view\remoteman-host-setup.exe";
/// The host's program in the guest, once installed.
pub const GUEST_HOST: &str = r"C:\Program Files\remoteman host\remoteman-host.exe";
/// The firewall rule's name in the guest.
pub const FIREWALL_RULE: &str = "Guesthouse remote view";
/// The only address the guest takes remoteman's port from: QEMU's, which is this PC.
pub const FROM: &str = "10.0.2.2";
/// What the take-over script of the display driver has once it tells the drivers apart by their
/// monitors (EDID) and not by their name, which remoteman's driver shares.
pub const TAKE_OVER_MARK: &str = r"MONITOR\GHS0001";

/// What the check script says.
pub const CURRENT: &str = "remote view: current";
pub const NEEDED: &str = "remote view: needed";
pub const OLD_TAKE_OVER: &str = "remote view: old take-over";
/// What the install script says once it is done.
pub const INSTALLED: &str = "remote view: installed";

/// remoteman's host installer at a release, as a host pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    pub url: String,
    pub file: String,
    pub sha256: String,
}

/// What the guest notes once installed: the version and the encoder.
pub fn note(version: &str, encoder: &str) -> String {
    format!("{version} {encoder}")
}

fn ps(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The script that says whether the installer has to run: [`CURRENT`], [`NEEDED`] (with the folder the
/// installer is sent to made), or
/// [`OLD_TAKE_OVER`] when the guest's take-over script would take remoteman's monitor for ours.
pub fn check_script(version: &str, encoder: &str) -> String {
    format!(
        "$take = 'C:\\gpupv\\idd\\take-over-display.ps1'\n\
         if ((Test-Path -LiteralPath $take) -and -not (Select-String -LiteralPath $take -SimpleMatch {mark} -Quiet)) {{ '{OLD_TAKE_OVER}'; return }}\n\
         $note = Join-Path {folder} 'installed.txt'\n\
         $have = if (Test-Path -LiteralPath $note) {{ (Get-Content -LiteralPath $note -Raw).Trim() }} else {{ '' }}\n\
         if ((Test-Path -LiteralPath {host}) -and $have -eq {want} -and (Get-Service 'remoteman host' -ea 0)) {{ '{CURRENT}'; return }}\n\
         New-Item -ItemType Directory -Force -Path {folder} | Out-Null\n\
         '{NEEDED}'\n",
        mark = ps(TAKE_OVER_MARK),
        folder = ps(GUEST_FOLDER),
        host = ps(GUEST_HOST),
        want = ps(&note(version, encoder)),
    )
}

/// The script that runs the installer sent to [`GUEST_INSTALLER`], lets remoteman's port in from
/// this PC only, and notes what it installed. It says [`INSTALLED`] and the host's fingerprint, or
/// what went wrong.
pub fn install_script(version: &str, encoder: &str) -> String {
    format!(
        "$folder = {folder}\n\
         New-Item -ItemType Directory -Force -Path $folder | Out-Null\n\
         $log = Join-Path $folder 'remoteman-setup.log'\n\
         $run = Start-Process -FilePath {installer} -ArgumentList '--silent','--no-pair','--encoder',{encoder},'--log',('\"' + $log + '\"') -Wait -PassThru\n\
         if ($run.ExitCode -ne 0) {{ \"remote view: the installer ended with $($run.ExitCode)\"; return }}\n\
         Get-NetFirewallRule -DisplayName {rule} -ea 0 | Remove-NetFirewallRule\n\
         New-NetFirewallRule -DisplayName {rule} -Direction Inbound -Action Allow -Protocol UDP -LocalPort 7791 -RemoteAddress {from} -Profile Any | Out-Null\n\
         Set-Content -LiteralPath (Join-Path $folder 'installed.txt') -Value {note} -Encoding ascii\n\
         Remove-Item -LiteralPath {installer} -ea 0\n\
         '{INSTALLED}'\n\
         & {host} fingerprint --state C:\\ProgramData\\remoteman\\host\n",
        folder = ps(GUEST_FOLDER),
        installer = ps(GUEST_INSTALLER),
        encoder = ps(encoder),
        rule = ps(FIREWALL_RULE),
        from = ps(FROM),
        note = ps(&note(version, encoder)),
        host = ps(GUEST_HOST),
    )
}

/// The script that runs one `ctl` command: the words are each quoted, never joined unquoted, as a
/// fingerprint comes from a person's device.
pub fn ctl_script(words: &[String]) -> String {
    let quoted: Vec<String> = words.iter().map(|word| ps(word)).collect();
    format!("& {} ctl --port {} {}\n", ps(GUEST_HOST), crate::PORT, quoted.join(" "))
}

/// The guest's fingerprint, from what [`install_script`] said.
pub fn fingerprint_after(said: &str) -> Option<String> {
    said.lines()
        .map(str::trim)
        .find(|line| line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_runs_with_no_trust_list_and_no_pairing() {
        let script = install_script("0.1.8", "d3d12");
        assert!(script.contains("'--silent','--no-pair','--encoder','d3d12'"));
        assert!(!script.contains("--trust"), "a person's devices are never replaced by a trust list");
        assert!(script.contains("-RemoteAddress '10.0.2.2'"), "only this PC reaches the port");
        assert!(script.contains("'0.1.8 d3d12'"));
    }

    #[test]
    fn the_check_wants_the_same_version_and_encoder_and_the_new_take_over() {
        let script = check_script("0.1.8", "auto");
        assert!(script.contains("'0.1.8 auto'"));
        assert!(script.contains(r"'MONITOR\GHS0001'"));
    }

    #[test]
    fn ctl_words_are_quoted_one_by_one() {
        let script = ctl_script(&["device".into(), "it's".into(), "allow".into()]);
        assert!(script.ends_with("ctl --port 7791 'device' 'it''s' 'allow'\n"), "{script}");
    }

    #[test]
    fn the_fingerprint_is_the_line_of_64_hex_digits() {
        let said = format!("{INSTALLED}\r\n{}\r\n", "AB".repeat(32));
        assert_eq!(fingerprint_after(&said), Some("ab".repeat(32)));
        assert_eq!(fingerprint_after(INSTALLED), None);
    }
}
