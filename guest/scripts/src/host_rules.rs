//! The firewall rules a Windows guest of the Windows host keeps for the ports Guesthouse lets in,
//! Remote Desktop's and the screen's (windows/host/guest/report.ps1), and the step the Linux host
//! sends to narrow remoteman's installer's own rule (shared/remote-view's `installer_rule_script`).
//! Windows Firewall and the guest's network are stood in for by functions, which are found before
//! the cmdlets of the same names, so nothing here reads or changes this PC's firewall. A rule's
//! history is kept, so a rule that was ever open to every address while on shows up.

use crate::{Said, Scratch, quote, run, shell};
use guesthouse_remote_view_core::install;
use std::fs;
use std::path::{Path, PathBuf};

const FUNCTIONS: [&str; 5] =
    ["Get-HostAddresses", "Get-Gateways", "Set-HostRule", "Limit-HostRule", "Set-RemoteDesktop"];

fn report() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../windows/host/guest/report.ps1")
}

/// The stand-ins. `$rules` holds each rule by its name; `$builtin` is Windows' own Remote Desktop
/// group, here with its name in German, as a guest in another language has it; `$routes` are the
/// default gateways, which arrive after `$routeAfter` seconds of waiting; `$refuse` makes every
/// change of a rule fail.
const FIREWALL: &str = "
    $rules = [ordered]@{}
    $history = New-Object System.Collections.Generic.List[string]
    $builtin = @(foreach ($name in 'RemoteDesktop-UserMode-In-TCP', 'RemoteDesktop-UserMode-In-UDP', 'RemoteDesktop-Shadow-In-TCP') {
        [pscustomobject]@{ Name = $name; Group = '@FirewallAPI.dll,-28752'; DisplayGroup = 'Remotedesktop'; Enabled = 'True' } })
    $refuse = $false
    $groupRead = 'ok'
    $routes = @()
    $routeAfter = 0
    $adapters = @('Ethernet')
    $global:slept = 0
    function Write-Rule([string]$Name) { $history.Add(\"$Name=$($rules[$Name].Remote) enabled $($rules[$Name].Enabled)\") }
    function Get-NetFirewallRule { [CmdletBinding()] param([string[]]$DisplayName, [string[]]$Group)
        if ($Group) {
            if ($groupRead -eq 'fails') { Write-Error -Message 'The RPC server is unavailable.' -Category ConnectionError; return }
            if ($groupRead -eq 'missing') { Write-Error -Message 'No rules in that group.' -Category ObjectNotFound; return }
            return $builtin | Where-Object { $Group -contains $_.Group } }
        foreach ($name in $DisplayName) { if ($rules.Contains($name)) { [pscustomobject]@{ DisplayName = $name; Enabled = $rules[$name].Enabled } } } }
    function New-NetFirewallRule { [CmdletBinding()] param([string]$DisplayName, [string]$Direction, [string]$Action, [string]$Protocol,
            [string[]]$LocalPort, [string[]]$RemoteAddress, [string]$Profile, [ValidateSet('True', 'False')][string]$Enabled = 'True')
        if ($refuse) { throw 'refused' }
        if (-not $RemoteAddress) { $RemoteAddress = @('Any') }
        $rules[$DisplayName] = @{ Remote = $RemoteAddress -join ' '; Enabled = $Enabled; What = \"$Direction $Action $Protocol $LocalPort $Profile\" }
        Write-Rule $DisplayName
        [pscustomobject]@{ DisplayName = $DisplayName } }
    function Set-NetFirewallRule { [CmdletBinding()] param([string[]]$DisplayName, [string]$Direction, [string]$Action, [string]$Protocol,
            [string[]]$LocalPort, [string[]]$RemoteAddress, [string]$Profile, [ValidateSet('True', 'False')][string]$Enabled)
        if ($refuse) { throw 'refused' }
        foreach ($name in $DisplayName) {
            if (-not $rules.Contains($name)) { throw \"no rule $name\" }
            if ($RemoteAddress) { $rules[$name].Remote = $RemoteAddress -join ' ' }
            if ($Enabled) { $rules[$name].Enabled = $Enabled }
            if ($Protocol) { $rules[$name].What = \"$Direction $Action $Protocol $LocalPort $Profile\" }
            Write-Rule $name } }
    function Disable-NetFirewallRule { [CmdletBinding()] param([string[]]$DisplayName, [string[]]$Group)
        if ($Group) { foreach ($rule in $builtin) { if ($Group -contains $rule.Group) { $rule.Enabled = 'False' } }; return }
        foreach ($name in $DisplayName) { $rules[$name].Enabled = 'False'; Write-Rule $name } }
    function Get-NetRoute { [CmdletBinding()] param([string]$DestinationPrefix)
        if ($global:slept -ge $routeAfter) { foreach ($hop in $routes) { [pscustomobject]@{ NextHop = $hop } } } }
    function Get-NetAdapter { [CmdletBinding()] param() $adapters }
    function Start-Sleep { param([int]$Seconds) $global:slept += $Seconds }
    function Show-Rules {
        foreach ($name in $rules.Keys) { \"$name=$($rules[$name].Remote) enabled $($rules[$name].Enabled)\" }
        foreach ($rule in $builtin) { \"$($rule.Name) enabled $($rule.Enabled)\" } }
    function Show-History { foreach ($line in $history) { \"was $line\" } }
";

/// A folder standing in for C:\Guesthouse, with hosts.txt as the host writes it, or none.
struct Guest {
    shell: &'static str,
    path: PathBuf,
    _scratch: Scratch,
}

impl Guest {
    fn new(shell: &'static str, hosts: Option<&str>) -> Self {
        let scratch = Scratch::new();
        let path = scratch.path("Guesthouse");
        fs::create_dir_all(&path).unwrap();
        if let Some(text) = hosts {
            fs::write(path.join("hosts.txt"), text).unwrap();
        }
        Self { shell, path, _scratch: scratch }
    }

    /// Runs `command` with the report's functions and the stand-ins loaded, `$file` the hosts file
    /// and `$note` the Remote Desktop note.
    fn run(&self, command: &str) -> Said {
        run(
            self.shell,
            &format!(
                "$ErrorActionPreference = 'Stop'
                {}
                {FIREWALL}
                $file = {}
                $note = {}
                {command}",
                crate::functions_from(&report(), &FUNCTIONS),
                quote(&self.path.join("hosts.txt")),
                quote(&self.path.join("remote-desktop.txt")),
            ),
        )
    }

    /// The rules after `command`, with what the functions said first.
    fn rules(&self, command: &str) -> Vec<String> {
        let said = self.run(&format!("{command}\nShow-Rules\nShow-History"));
        assert_eq!(said.code, Some(0), "{said}");
        said.lines().into_iter().map(str::to_owned).collect()
    }
}

/// The lines of `lines` that say a rule was on and open to every address, at any time.
fn ever_open(lines: &[String]) -> Vec<&String> {
    lines.iter().filter(|line| line.contains("=Any enabled True")).collect()
}

/// On the shared network a port forward of this PC's reaches the guest at 172.29.1.2 from this PC's
/// end of that network, 172.29.1.1, the guest's default gateway, which the host's file doesn't name:
/// Remote Desktop lets it in with this PC's own addresses. Windows' own rules, in German here, go off
/// by their group.
#[test]
fn remote_desktop_lets_in_this_pc_and_the_address_its_forwards_come_from() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("shared\n192.168.1.20\n"));
    let lines = guest.rules(
        "$routes = @('172.29.1.1')
        foreach ($said in @(Set-RemoteDesktop @(Get-HostAddresses $file (Get-Gateways 30)) 'Test Remote Desktop' $note)) { \"said $said\" }",
    );
    assert!(lines.contains(&"said Windows' own rules are off (3 were on)".to_owned()), "{lines:#?}");
    let rules: Vec<&str> = lines
        .iter()
        .filter(|line| !line.starts_with("said") && !line.starts_with("was"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        rules,
        [
            "Test Remote Desktop=192.168.1.20 172.29.1.1 enabled True",
            "RemoteDesktop-UserMode-In-TCP enabled False",
            "RemoteDesktop-UserMode-In-UDP enabled False",
            "RemoteDesktop-Shadow-In-TCP enabled False",
        ],
        "{lines:#?}"
    );
    assert!(ever_open(&lines).is_empty(), "{lines:#?}");
    assert!(guest.path.join("remote-desktop.txt").is_file());
}

/// The guest's gateway can come after the report starts: it is waited for, and let in once there.
#[test]
fn a_gateway_that_comes_late_is_waited_for() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("shared\n192.168.1.20\n"));
    let lines = guest.rules(
        "$routes = @('172.29.1.1'); $routeAfter = 6
        Set-RemoteDesktop @(Get-HostAddresses $file (Get-Gateways 30)) 'Test Remote Desktop' $note | Out-Null
        \"slept $global:slept\"",
    );
    assert!(lines.contains(&"slept 6".to_owned()), "{lines:#?}");
    assert!(
        lines.contains(&"Test Remote Desktop=192.168.1.20 172.29.1.1 enabled True".to_owned()),
        "{lines:#?}"
    );
}

/// A guest with no network adapter isn't held up waiting for a gateway.
#[test]
fn a_guest_with_no_network_waits_for_nothing() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("shared\n192.168.1.20\n"));
    let lines = guest.rules("$adapters = @(); Get-Gateways 30 | Out-Null; \"slept $global:slept\"");
    assert!(lines.contains(&"slept 0".to_owned()), "{lines:#?}");
}

/// A bridged guest lets in this PC's addresses on that network, so Remote Desktop from this PC
/// works, and never its router, the guest's gateway there.
#[test]
fn a_bridged_guest_lets_in_this_pc_and_not_its_router() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("bridged\n192.168.1.20\n100.64.1.2\n"));
    let lines = guest.rules(
        "$routes = @('192.168.1.1')
        Set-RemoteDesktop @(Get-HostAddresses $file (Get-Gateways 30)) 'Test Remote Desktop' $note | Out-Null",
    );
    assert!(
        lines.contains(&"Test Remote Desktop=192.168.1.20 100.64.1.2 enabled True".to_owned()),
        "{lines:#?}"
    );
}

/// With no address of this PC's known, rules that were open are turned off and none is made open:
/// no file, a network the guest doesn't know, a file with nothing in it, and bridged with no IPv4
/// address on this PC.
#[test]
fn with_no_address_known_every_rule_is_off() {
    let Some(shell) = shell() else { return };
    for hosts in
        [None, Some("tunnel\n192.168.1.20\n"), Some(""), Some("bridged\n"), Some("bridged\nfe80::1\n")]
    {
        let guest = Guest::new(shell, hosts);
        let lines = guest.rules(
            "$routes = @('192.168.1.1')
            $rules['Test screen'] = @{ Remote = 'Any'; Enabled = 'True' }
            $rules['Test remoteman'] = @{ Remote = 'Any'; Enabled = 'True' }
            $allowed = @(Get-HostAddresses $file (Get-Gateways 30))
            \"said $(Set-HostRule 'Test screen' 'UDP' 7791 $allowed)\"
            \"said $(Limit-HostRule 'Test remoteman' $allowed)\"
            \"said $(Set-RemoteDesktop $allowed 'Test Remote Desktop' $note)\"",
        );
        let rules: Vec<&str> = lines
            .iter()
            .filter(|line| line.contains("=") && !line.starts_with("was"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            rules,
            ["Test screen=Any enabled False", "Test remoteman=Any enabled False"],
            "{hosts:?}: no rule is on, and none is made: {lines:#?}"
        );
    }
}

/// A rule that can't be set is turned off, never left open.
#[test]
fn a_rule_that_cant_be_set_is_turned_off() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("bridged\n192.168.1.20\n"));
    let lines = guest.rules(
        "$rules['Test screen'] = @{ Remote = 'Any'; Enabled = 'True' }
        $rules['Test remoteman'] = @{ Remote = 'Any'; Enabled = 'True' }
        $refuse = $true
        $allowed = @(Get-HostAddresses $file @())
        \"said $(Set-HostRule 'Test screen' 'UDP' 7791 $allowed)\"
        \"said $(Limit-HostRule 'Test remoteman' $allowed)\"",
    );
    assert!(
        lines.contains(&"said Test screen is off, as it couldn't be set: refused".to_owned()),
        "{lines:#?}"
    );
    assert!(lines.contains(&"said Test remoteman is off: refused".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"Test screen=Any enabled False".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"Test remoteman=Any enabled False".to_owned()), "{lines:#?}");
}

/// A rule made new is made with its addresses, and an open one is narrowed: neither is ever on and
/// open to every address. remoteman's installer opens its rule again each time it runs, and the next
/// start narrows it again.
#[test]
fn rules_are_made_and_narrowed_with_their_addresses() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("shared\n192.168.1.20\n"));
    let lines = guest.rules(
        "$rules['Test remoteman'] = @{ Remote = 'Any'; Enabled = 'True' }
        $routes = @('172.29.1.1')
        $allowed = @(Get-HostAddresses $file (Get-Gateways 0))
        Set-HostRule 'Test screen' 'UDP' 7791 $allowed | Out-Null
        Limit-HostRule 'Test remoteman' $allowed | Out-Null
        $rules['Test remoteman'] = @{ Remote = 'Any'; Enabled = 'True' }
        Limit-HostRule 'Test remoteman' $allowed | Out-Null
        \"said $(Limit-HostRule 'Not there' $allowed)\"",
    );
    assert!(lines.contains(&"Test screen=192.168.1.20 172.29.1.1 enabled True".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"Test remoteman=192.168.1.20 172.29.1.1 enabled True".to_owned()), "{lines:#?}");
    assert!(lines.contains(&"said Not there isn't there".to_owned()), "{lines:#?}");
    assert!(ever_open(&lines).is_empty(), "{lines:#?}");
}

/// Windows' own Remote Desktop rules are turned off once: turned on again by hand later, they stay.
#[test]
fn windows_own_remote_desktop_rules_are_turned_off_once() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("bridged\n192.168.1.20\n"));
    let lines = guest.rules(
        "Set-RemoteDesktop @(Get-HostAddresses $file @()) 'Test Remote Desktop' $note | Out-Null
        foreach ($rule in $builtin) { $rule.Enabled = 'True' }
        foreach ($said in @(Set-RemoteDesktop @(Get-HostAddresses $file @()) 'Test Remote Desktop' $note)) { \"said $said\" }",
    );
    assert!(
        lines.contains(&"said Test Remote Desktop lets TCP 3389 in from 192.168.1.20".to_owned()),
        "{lines:#?}"
    );
    assert!(!lines.iter().any(|line| line.contains("Windows' own rules")), "{lines:#?}");
    assert!(lines.contains(&"RemoteDesktop-UserMode-In-TCP enabled True".to_owned()), "{lines:#?}");
}

/// When Windows' own group can't be read, nothing is noted, so it is tried again at the next start;
/// a group Windows doesn't have counts as done.
#[test]
fn windows_own_rules_are_tried_again_when_they_couldnt_be_read() {
    let Some(shell) = shell() else { return };
    let guest = Guest::new(shell, Some("bridged\n192.168.1.20\n"));
    let lines = guest.rules(
        "$groupRead = 'fails'
        foreach ($said in @(Set-RemoteDesktop @(Get-HostAddresses $file @()) 'Test Remote Desktop' $note)) { \"said $said\" }
        \"noted $(Test-Path -LiteralPath $note)\"
        $groupRead = 'missing'
        foreach ($said in @(Set-RemoteDesktop @(Get-HostAddresses $file @()) 'Test Remote Desktop' $note)) { \"said $said\" }
        \"noted $(Test-Path -LiteralPath $note)\"",
    );
    let said: Vec<&str> = lines
        .iter()
        .filter(|line| line.starts_with("said") || line.starts_with("noted"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        said,
        [
            "said Windows' own rules couldn't be turned off: The RPC server is unavailable.",
            "said Test Remote Desktop lets TCP 3389 in from 192.168.1.20",
            "noted False",
            "said Windows' own rules are off (0 were on)",
            "said Test Remote Desktop lets TCP 3389 in from 192.168.1.20",
            "noted True",
        ],
        "{lines:#?}"
    );
}

/// The Linux host's step for remoteman's installer's rule: narrowed to QEMU's address, or off when
/// it can't be set, and nothing said either way, as the check's answer is read word for word.
#[test]
fn the_linux_hosts_step_narrows_remotemans_rule_or_turns_it_off() {
    let Some(shell) = shell() else { return };
    let script = install::installer_rule_script().replace(install::INSTALLER_RULE, "Test remoteman");
    for (refuse, after) in
        [("$false", "Test remoteman=10.0.2.2 enabled True"), ("$true", "Test remoteman=Any enabled False")]
    {
        let said = run(
            shell,
            &format!(
                "$ErrorActionPreference = 'Stop'
                {FIREWALL}
                $rules['Test remoteman'] = @{{ Remote = 'Any'; Enabled = 'True' }}
                $refuse = {refuse}
                {script}
                Show-Rules"
            ),
        );
        assert_eq!(said.code, Some(0), "{said}");
        assert_eq!(said.lines()[0], after, "{said}");
    }
    let said = run(shell, &format!("$ErrorActionPreference = 'Stop'\n{FIREWALL}\n{script}\n'done'"));
    assert_eq!(said.lines(), ["done"], "with no such rule it says nothing: {said}");
}
